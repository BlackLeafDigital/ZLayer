# NetworkAttachmentInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Aliases** | Pointer to **[]string** | DNS aliases the container answers to on this network. | [optional] 
**Ipv4** | Pointer to **NullableString** | Assigned IPv4 on this network, if any. | [optional] 
**Network** | **string** | Network name as reported by the runtime. Matches the &#x60;name&#x60; field on entries returned by &#x60;GET /api/v1/container-networks&#x60;. | 

## Methods

### NewNetworkAttachmentInfo

`func NewNetworkAttachmentInfo(network string, ) *NetworkAttachmentInfo`

NewNetworkAttachmentInfo instantiates a new NetworkAttachmentInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNetworkAttachmentInfoWithDefaults

`func NewNetworkAttachmentInfoWithDefaults() *NetworkAttachmentInfo`

NewNetworkAttachmentInfoWithDefaults instantiates a new NetworkAttachmentInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAliases

`func (o *NetworkAttachmentInfo) GetAliases() []string`

GetAliases returns the Aliases field if non-nil, zero value otherwise.

### GetAliasesOk

`func (o *NetworkAttachmentInfo) GetAliasesOk() (*[]string, bool)`

GetAliasesOk returns a tuple with the Aliases field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAliases

`func (o *NetworkAttachmentInfo) SetAliases(v []string)`

SetAliases sets Aliases field to given value.

### HasAliases

`func (o *NetworkAttachmentInfo) HasAliases() bool`

HasAliases returns a boolean if a field has been set.

### GetIpv4

`func (o *NetworkAttachmentInfo) GetIpv4() string`

GetIpv4 returns the Ipv4 field if non-nil, zero value otherwise.

### GetIpv4Ok

`func (o *NetworkAttachmentInfo) GetIpv4Ok() (*string, bool)`

GetIpv4Ok returns a tuple with the Ipv4 field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIpv4

`func (o *NetworkAttachmentInfo) SetIpv4(v string)`

SetIpv4 sets Ipv4 field to given value.

### HasIpv4

`func (o *NetworkAttachmentInfo) HasIpv4() bool`

HasIpv4 returns a boolean if a field has been set.

### SetIpv4Nil

`func (o *NetworkAttachmentInfo) SetIpv4Nil(b bool)`

 SetIpv4Nil sets the value for Ipv4 to be an explicit nil

### UnsetIpv4
`func (o *NetworkAttachmentInfo) UnsetIpv4()`

UnsetIpv4 ensures that no value is present for Ipv4, not even an explicit nil
### GetNetwork

`func (o *NetworkAttachmentInfo) GetNetwork() string`

GetNetwork returns the Network field if non-nil, zero value otherwise.

### GetNetworkOk

`func (o *NetworkAttachmentInfo) GetNetworkOk() (*string, bool)`

GetNetworkOk returns a tuple with the Network field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNetwork

`func (o *NetworkAttachmentInfo) SetNetwork(v string)`

SetNetwork sets Network field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


