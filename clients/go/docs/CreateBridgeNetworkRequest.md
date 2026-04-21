# CreateBridgeNetworkRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Driver** | Pointer to [**NullableBridgeNetworkDriver**](BridgeNetworkDriver.md) | Driver, defaults to &#x60;bridge&#x60;. | [optional] 
**Internal** | Pointer to **bool** | Internal-only (no egress) network. | [optional] 
**Labels** | Pointer to **map[string]string** | Arbitrary labels. | [optional] 
**Name** | **string** | Network name (must match &#x60;^[a-z0-9][a-z0-9_-]{0,63}$&#x60;). | 
**Subnet** | Pointer to **NullableString** | Subnet CIDR (e.g. &#x60;\&quot;10.240.0.0/24\&quot;&#x60;). Validated as [&#x60;ipnetwork::IpNetwork&#x60;]. | [optional] 

## Methods

### NewCreateBridgeNetworkRequest

`func NewCreateBridgeNetworkRequest(name string, ) *CreateBridgeNetworkRequest`

NewCreateBridgeNetworkRequest instantiates a new CreateBridgeNetworkRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateBridgeNetworkRequestWithDefaults

`func NewCreateBridgeNetworkRequestWithDefaults() *CreateBridgeNetworkRequest`

NewCreateBridgeNetworkRequestWithDefaults instantiates a new CreateBridgeNetworkRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDriver

`func (o *CreateBridgeNetworkRequest) GetDriver() BridgeNetworkDriver`

GetDriver returns the Driver field if non-nil, zero value otherwise.

### GetDriverOk

`func (o *CreateBridgeNetworkRequest) GetDriverOk() (*BridgeNetworkDriver, bool)`

GetDriverOk returns a tuple with the Driver field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDriver

`func (o *CreateBridgeNetworkRequest) SetDriver(v BridgeNetworkDriver)`

SetDriver sets Driver field to given value.

### HasDriver

`func (o *CreateBridgeNetworkRequest) HasDriver() bool`

HasDriver returns a boolean if a field has been set.

### SetDriverNil

`func (o *CreateBridgeNetworkRequest) SetDriverNil(b bool)`

 SetDriverNil sets the value for Driver to be an explicit nil

### UnsetDriver
`func (o *CreateBridgeNetworkRequest) UnsetDriver()`

UnsetDriver ensures that no value is present for Driver, not even an explicit nil
### GetInternal

`func (o *CreateBridgeNetworkRequest) GetInternal() bool`

GetInternal returns the Internal field if non-nil, zero value otherwise.

### GetInternalOk

`func (o *CreateBridgeNetworkRequest) GetInternalOk() (*bool, bool)`

GetInternalOk returns a tuple with the Internal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetInternal

`func (o *CreateBridgeNetworkRequest) SetInternal(v bool)`

SetInternal sets Internal field to given value.

### HasInternal

`func (o *CreateBridgeNetworkRequest) HasInternal() bool`

HasInternal returns a boolean if a field has been set.

### GetLabels

`func (o *CreateBridgeNetworkRequest) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *CreateBridgeNetworkRequest) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *CreateBridgeNetworkRequest) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *CreateBridgeNetworkRequest) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetName

`func (o *CreateBridgeNetworkRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateBridgeNetworkRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateBridgeNetworkRequest) SetName(v string)`

SetName sets Name field to given value.


### GetSubnet

`func (o *CreateBridgeNetworkRequest) GetSubnet() string`

GetSubnet returns the Subnet field if non-nil, zero value otherwise.

### GetSubnetOk

`func (o *CreateBridgeNetworkRequest) GetSubnetOk() (*string, bool)`

GetSubnetOk returns a tuple with the Subnet field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSubnet

`func (o *CreateBridgeNetworkRequest) SetSubnet(v string)`

SetSubnet sets Subnet field to given value.

### HasSubnet

`func (o *CreateBridgeNetworkRequest) HasSubnet() bool`

HasSubnet returns a boolean if a field has been set.

### SetSubnetNil

`func (o *CreateBridgeNetworkRequest) SetSubnetNil(b bool)`

 SetSubnetNil sets the value for Subnet to be an explicit nil

### UnsetSubnet
`func (o *CreateBridgeNetworkRequest) UnsetSubnet()`

UnsetSubnet ensures that no value is present for Subnet, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


