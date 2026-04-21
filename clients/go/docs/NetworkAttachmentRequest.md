# NetworkAttachmentRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Aliases** | Pointer to **[]string** | Optional DNS aliases for this container on the network. | [optional] 
**Ipv4Address** | Pointer to **NullableString** | Optional static IPv4 to pin this container to. Validated as [&#x60;std::net::Ipv4Addr&#x60;] before the runtime is called. | [optional] 
**Network** | **string** | Bridge-network id or name to attach to. | 

## Methods

### NewNetworkAttachmentRequest

`func NewNetworkAttachmentRequest(network string, ) *NetworkAttachmentRequest`

NewNetworkAttachmentRequest instantiates a new NetworkAttachmentRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNetworkAttachmentRequestWithDefaults

`func NewNetworkAttachmentRequestWithDefaults() *NetworkAttachmentRequest`

NewNetworkAttachmentRequestWithDefaults instantiates a new NetworkAttachmentRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAliases

`func (o *NetworkAttachmentRequest) GetAliases() []string`

GetAliases returns the Aliases field if non-nil, zero value otherwise.

### GetAliasesOk

`func (o *NetworkAttachmentRequest) GetAliasesOk() (*[]string, bool)`

GetAliasesOk returns a tuple with the Aliases field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAliases

`func (o *NetworkAttachmentRequest) SetAliases(v []string)`

SetAliases sets Aliases field to given value.

### HasAliases

`func (o *NetworkAttachmentRequest) HasAliases() bool`

HasAliases returns a boolean if a field has been set.

### GetIpv4Address

`func (o *NetworkAttachmentRequest) GetIpv4Address() string`

GetIpv4Address returns the Ipv4Address field if non-nil, zero value otherwise.

### GetIpv4AddressOk

`func (o *NetworkAttachmentRequest) GetIpv4AddressOk() (*string, bool)`

GetIpv4AddressOk returns a tuple with the Ipv4Address field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIpv4Address

`func (o *NetworkAttachmentRequest) SetIpv4Address(v string)`

SetIpv4Address sets Ipv4Address field to given value.

### HasIpv4Address

`func (o *NetworkAttachmentRequest) HasIpv4Address() bool`

HasIpv4Address returns a boolean if a field has been set.

### SetIpv4AddressNil

`func (o *NetworkAttachmentRequest) SetIpv4AddressNil(b bool)`

 SetIpv4AddressNil sets the value for Ipv4Address to be an explicit nil

### UnsetIpv4Address
`func (o *NetworkAttachmentRequest) UnsetIpv4Address()`

UnsetIpv4Address ensures that no value is present for Ipv4Address, not even an explicit nil
### GetNetwork

`func (o *NetworkAttachmentRequest) GetNetwork() string`

GetNetwork returns the Network field if non-nil, zero value otherwise.

### GetNetworkOk

`func (o *NetworkAttachmentRequest) GetNetworkOk() (*string, bool)`

GetNetworkOk returns a tuple with the Network field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNetwork

`func (o *NetworkAttachmentRequest) SetNetwork(v string)`

SetNetwork sets Network field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


