# BridgeNetworkAttachment

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Aliases** | Pointer to **[]string** | DNS aliases the container can be reached by on this network. | [optional] 
**ContainerId** | **string** | Runtime-provided container id. | 
**ContainerName** | Pointer to **NullableString** | Container name, if known. | [optional] 
**Ipv4** | Pointer to **NullableString** | Assigned IPv4 address on the network (if any). | [optional] 

## Methods

### NewBridgeNetworkAttachment

`func NewBridgeNetworkAttachment(containerId string, ) *BridgeNetworkAttachment`

NewBridgeNetworkAttachment instantiates a new BridgeNetworkAttachment object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBridgeNetworkAttachmentWithDefaults

`func NewBridgeNetworkAttachmentWithDefaults() *BridgeNetworkAttachment`

NewBridgeNetworkAttachmentWithDefaults instantiates a new BridgeNetworkAttachment object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAliases

`func (o *BridgeNetworkAttachment) GetAliases() []string`

GetAliases returns the Aliases field if non-nil, zero value otherwise.

### GetAliasesOk

`func (o *BridgeNetworkAttachment) GetAliasesOk() (*[]string, bool)`

GetAliasesOk returns a tuple with the Aliases field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAliases

`func (o *BridgeNetworkAttachment) SetAliases(v []string)`

SetAliases sets Aliases field to given value.

### HasAliases

`func (o *BridgeNetworkAttachment) HasAliases() bool`

HasAliases returns a boolean if a field has been set.

### GetContainerId

`func (o *BridgeNetworkAttachment) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *BridgeNetworkAttachment) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *BridgeNetworkAttachment) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.


### GetContainerName

`func (o *BridgeNetworkAttachment) GetContainerName() string`

GetContainerName returns the ContainerName field if non-nil, zero value otherwise.

### GetContainerNameOk

`func (o *BridgeNetworkAttachment) GetContainerNameOk() (*string, bool)`

GetContainerNameOk returns a tuple with the ContainerName field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerName

`func (o *BridgeNetworkAttachment) SetContainerName(v string)`

SetContainerName sets ContainerName field to given value.

### HasContainerName

`func (o *BridgeNetworkAttachment) HasContainerName() bool`

HasContainerName returns a boolean if a field has been set.

### SetContainerNameNil

`func (o *BridgeNetworkAttachment) SetContainerNameNil(b bool)`

 SetContainerNameNil sets the value for ContainerName to be an explicit nil

### UnsetContainerName
`func (o *BridgeNetworkAttachment) UnsetContainerName()`

UnsetContainerName ensures that no value is present for ContainerName, not even an explicit nil
### GetIpv4

`func (o *BridgeNetworkAttachment) GetIpv4() string`

GetIpv4 returns the Ipv4 field if non-nil, zero value otherwise.

### GetIpv4Ok

`func (o *BridgeNetworkAttachment) GetIpv4Ok() (*string, bool)`

GetIpv4Ok returns a tuple with the Ipv4 field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIpv4

`func (o *BridgeNetworkAttachment) SetIpv4(v string)`

SetIpv4 sets Ipv4 field to given value.

### HasIpv4

`func (o *BridgeNetworkAttachment) HasIpv4() bool`

HasIpv4 returns a boolean if a field has been set.

### SetIpv4Nil

`func (o *BridgeNetworkAttachment) SetIpv4Nil(b bool)`

 SetIpv4Nil sets the value for Ipv4 to be an explicit nil

### UnsetIpv4
`func (o *BridgeNetworkAttachment) UnsetIpv4()`

UnsetIpv4 ensures that no value is present for Ipv4, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


